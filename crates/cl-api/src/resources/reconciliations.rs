//! Reconcile (P27): the worksheet the engine keeps for one statement of a
//! bank or card account, and its Begin, tick, finish, undo and reports
//! routes (engine U-188 `reconcile-exact-v1`, U-189 `reconcile-begin-v1`,
//! U-190 `reconcile-undo-v2`, U-191 `reconcile-discrepancy-v1`).
//!
//! - **The engine owns the arithmetic.** Every balance, total and the
//!   difference comes back from the engine as an exact decimal string and
//!   is parsed into [`Money`]; a JSON number where an amount belongs is a
//!   `Decode` error (it could have been rounded). Nothing here adds amounts.
//! - Amounts are in the **books sign** of the account (a card's balance
//!   owed is negative); the screen flips a card's figures as text only.
//!   The owner's own statement figures are sent as typed: a card's printed
//!   balance is positive and the engine flips it (Start and PATCH).
//! - Ticks go in one batch (`PUT /lines`, at most [`LINES_MAX`]) and the
//!   answer is the header and totals only (`?view=summary`).
//! - Complete and undo are not keyed: a retry after a lost answer gets a
//!   409 and the screen reads the reconciliation again.
//! - Refusals are named by [`reconcile_refusal`] from the engine's code.
//! - The reports are display text ([`ReportAmount`]): never parsed to a
//!   float, summed or compared.

use std::fmt;

use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::customer_payments::error_code;
use crate::resources::operations::{
    Operation, RECON_CANCEL, RECON_COMPLETE, RECON_DETAIL_REPORT, RECON_DISCREPANCY, RECON_GET,
    RECON_LINES, RECON_PATCH, RECON_REFRESH, RECON_START, RECON_UNDO, RECONCILIATIONS,
};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::session::{Session, json_body};
use crate::wire::{Capabilities, ReportAmount};

/// The most ticks one `PUT /lines` takes (the engine allows 10,000).
pub const LINES_MAX: usize = 10_000;

/// A reconciliation's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReconciliationId(uuid::Uuid);

impl ReconciliationId {
    /// Parses a reconciliation id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<ReconciliationId, ApiError> {
        cl_core::ids::parse_uuid(s)
            .map(ReconciliationId)
            .map_err(|_| ApiError::Decode {
                what: "reconciliation id",
            })
    }

    /// A fixed id (the demo and tests).
    #[must_use]
    pub const fn from_uuid(u: uuid::Uuid) -> ReconciliationId {
        ReconciliationId(u)
    }
}

impl fmt::Display for ReconciliationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A reconciliation's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReconStatus {
    /// Started and not finished.
    InProgress,
    /// Finished.
    Complete,
    /// A state this client doesn't know.
    Other,
}

impl ReconStatus {
    fn parse(s: Option<&str>) -> ReconStatus {
        match s {
            Some("in_progress") | None => ReconStatus::InProgress,
            Some("complete") => ReconStatus::Complete,
            Some(_) => ReconStatus::Other,
        }
    }
}

/// A service charge or interest entered at Begin (U-189).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StatementCharge {
    /// The amount (> 0, at most 2 decimals when sent).
    pub amount: Money,
    /// The entry's date (the statement date by default).
    pub date: CalendarDate,
    /// The expense (charge) or income (interest) account.
    pub account: AccountId,
}

impl fmt::Debug for StatementCharge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementCharge")
            .field("date", &self.date)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A charge as the reconciliation keeps it: posted at Finish, then never
/// again (after an undo it stays in the books).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldCharge {
    /// What was entered.
    pub charge: StatementCharge,
    /// The posted entry, once Finish posted it.
    pub posted: Option<TransactionId>,
}

/// A count and the engine's total of the cleared lines on one side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClearedSide {
    /// Lines ticked (pending charges included).
    pub count: u32,
    /// Their total as the engine sums it (positive).
    pub total: Money,
}

/// The beginning balance moved since the last finished reconciliation
/// (U-189 drift).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeginningDrift {
    /// The last reconciliation's ending balance.
    pub expected: Money,
    /// What its cleared lines add up to today.
    pub actual: Money,
    /// `actual - expected` as the engine reports it.
    pub difference: Money,
}

/// A statement whose opening balance doesn't match the last ending
/// balance (statement-driven starts).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Continuity {
    /// The last ending balance.
    pub expected: Money,
    /// The statement's opening balance.
    pub actual: Money,
    /// The gap.
    pub delta: Money,
}

/// One worksheet line: a journal line on the account. (Its flags are
/// independent facts the engine reports, hence the bools.)
#[derive(Clone, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct ReconLine {
    /// The journal line (what a tick names).
    pub journal_line: JournalLineId,
    /// Its transaction (Ctrl+E opens it).
    pub transaction: TransactionId,
    /// Ticked on this worksheet.
    pub cleared: bool,
    /// The entry's date.
    pub date: CalendarDate,
    /// The entry's kind (`check`, `deposit`, ...).
    pub txn_type: String,
    /// The check or reference number.
    pub number: Option<String>,
    /// The payee or a short description.
    pub payee: Option<String>,
    /// Debit − credit, in the books sign (positive = money in for a bank).
    pub amount: Money,
    /// Dated after the statement date (shown only with the toggle off).
    pub after_statement_date: bool,
    /// Accepted or matched from a bank file (pre-checked, Q-100).
    pub from_bank_feed: bool,
    /// The engine thinks it doubles a cleared line.
    pub likely_duplicate: bool,
}

impl fmt::Debug for ReconLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReconLine")
            .field("journal_line", &self.journal_line)
            .field("cleared", &self.cleared)
            .field("date", &self.date)
            .field("from_bank_feed", &self.from_bank_feed)
            .finish_non_exhaustive()
    }
}

/// The worksheet as the engine reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reconciliation {
    /// Its id.
    pub id: ReconciliationId,
    /// The account reconciled.
    pub account: AccountId,
    /// In progress or finished.
    pub status: ReconStatus,
    /// The statement date.
    pub statement_date: CalendarDate,
    /// The beginning balance (books sign).
    pub beginning_balance: Money,
    /// The statement's ending balance (books sign).
    pub statement_ending_balance: Money,
    /// Beginning + every ticked line + pending charges.
    pub cleared_balance: Money,
    /// Ending − cleared. Finish needs exactly zero.
    pub difference: Money,
    /// Ticked money in.
    pub cleared_deposits: ClearedSide,
    /// Ticked money out.
    pub cleared_payments: ClearedSide,
    /// The service charge (a card's finance charge), when entered.
    pub service_charge: Option<HeldCharge>,
    /// The interest earned, when entered.
    pub interest: Option<HeldCharge>,
    /// The worksheet holds lines after the statement date too.
    pub include_later_lines: bool,
    /// The beginning balance moved since the last reconciliation.
    pub drift: Option<BeginningDrift>,
    /// The statement's opening balance doesn't follow on.
    pub continuity: Option<Continuity>,
    /// A pending charge is dated in a closed period: Finish needs the
    /// closing-date password.
    pub posting_locked: bool,
    /// When it was started, when the engine said.
    pub started: Option<Timestamp>,
    /// The lines (`None` for a summary answer).
    pub lines: Option<Vec<ReconLine>>,
}

impl Reconciliation {
    /// True when the engine's difference is exactly zero.
    #[must_use]
    pub fn ties_out(&self) -> bool {
        self.difference.is_zero()
    }
}

/// One row of an account's reconciliation history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconHistoryRow {
    /// Its id.
    pub id: ReconciliationId,
    /// In progress or finished.
    pub status: ReconStatus,
    /// The statement date.
    pub statement_date: CalendarDate,
    /// The statement's ending balance (books sign), when an exact string.
    pub statement_ending_balance: Option<Money>,
    /// When it was finished.
    pub completed: Option<Timestamp>,
    /// When it was started.
    pub started: Option<Timestamp>,
}

/// What Begin sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconStart {
    /// The account.
    pub account: AccountId,
    /// The statement date.
    pub statement_date: CalendarDate,
    /// The statement's ending balance as printed (a card's owed is
    /// positive; the engine flips it).
    pub statement_ending_balance: Money,
    /// The service charge (a card's finance charge).
    pub service_charge: Option<StatementCharge>,
    /// The interest earned.
    pub interest: Option<StatementCharge>,
}

/// A header change ("Change statement info"). `None` leaves a field as
/// it is; `Some(None)` removes a charge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconPatch {
    /// A new statement date.
    pub statement_date: Option<CalendarDate>,
    /// A new ending balance as printed.
    pub statement_ending_balance: Option<Money>,
    /// The service charge.
    pub service_charge: Option<Option<StatementCharge>>,
    /// The interest.
    pub interest: Option<Option<StatementCharge>>,
}

/// How Undo leaves the reconciliation (U-190).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoMode {
    /// Reopen it with the ticks exactly as they were at Finish.
    KeepChecks,
    /// Take it away entirely (the lines go back to uncleared).
    Discard,
}

impl UndoMode {
    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            UndoMode::KeepChecks => "keep_checks",
            UndoMode::Discard => "discard",
        }
    }
}

/// A named refusal of a reconciliation route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileRefusal {
    /// Finish with a difference that isn't exactly zero.
    DifferenceNotZero {
        /// The engine's difference, when an exact string.
        difference: Option<Money>,
    },
    /// Another reconciliation of the account is in progress.
    InProgress {
        /// That reconciliation.
        reconciliation: Option<ReconciliationId>,
    },
    /// This reconciliation is no longer in progress (finished or
    /// discarded elsewhere).
    NotInProgress,
    /// Only the most recent finished reconciliation can be undone.
    NotLatest,
    /// The charge was already posted; change it in the register.
    ChargePosted,
    /// Undo needs an Admin.
    AdminRequired,
}

/// Names the refusal behind `e`, if it is a reconciliation one.
#[must_use]
pub fn reconcile_refusal(e: &ApiError) -> Option<ReconcileRefusal> {
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        ApiError::Validation(v) => Some(v.details()),
        _ => None,
    };
    let text = |k: &str| details.and_then(|d| d.text(k)).map(str::to_string);
    let code = match e {
        ApiError::Forbidden { code: Some(c) } => Some(c.as_str()),
        _ => error_code(e),
    }?;
    Some(match code {
        "RECONCILE_DIFFERENCE_NOT_ZERO" => ReconcileRefusal::DifferenceNotZero {
            difference: text("difference").and_then(|s| Money::parse_wire(&s).ok()),
        },
        "RECONCILIATION_IN_PROGRESS" => ReconcileRefusal::InProgress {
            reconciliation: text("reconciliationId").and_then(|s| ReconciliationId::parse(&s).ok()),
        },
        "RECONCILIATION_NOT_IN_PROGRESS" => ReconcileRefusal::NotInProgress,
        "RECONCILIATION_NOT_LATEST" => ReconcileRefusal::NotLatest,
        "RECONCILE_CHARGE_POSTED" => ReconcileRefusal::ChargePosted,
        "RECONCILE_ADMIN_REQUIRED" => ReconcileRefusal::AdminRequired,
        _ => return None,
    })
}

/// A row of the reconciliation discrepancy report (U-191).
#[derive(Clone, PartialEq, Eq)]
pub struct DiscrepancyRow {
    /// What happened (`changed`, `voided`, `deleted`, `added_after`,
    /// `reconciliation_undone`).
    pub kind: String,
    /// The statement date of the reconciliation affected.
    pub statement_date: Option<CalendarDate>,
    /// The entry's date.
    pub txn_date: Option<CalendarDate>,
    /// The entry's kind.
    pub txn_type: Option<String>,
    /// Its number.
    pub number: Option<String>,
    /// Its payee.
    pub payee: Option<String>,
    /// The amount when reconciled.
    pub reconciled_amount: Option<Money>,
    /// The change to the reconciled amount.
    pub effect: Option<Money>,
    /// When it changed.
    pub changed_at: Option<Timestamp>,
    /// Who changed it.
    pub changed_by: Option<String>,
    /// The fields changed (names only).
    pub fields: Vec<String>,
    /// The entry, when there still is one.
    pub transaction: Option<TransactionId>,
}

impl fmt::Debug for DiscrepancyRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiscrepancyRow")
            .field("kind", &self.kind)
            .field("fields", &self.fields)
            .finish_non_exhaustive()
    }
}

/// The discrepancy report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscrepancyReport {
    /// Its rows, as the engine orders them.
    pub rows: Vec<DiscrepancyRow>,
    /// The beginning balance drift, if any.
    pub drift: Option<BeginningDrift>,
}

/// One line of the detail report (display text only).
#[derive(Clone, Debug)]
pub struct ReportLine {
    /// The entry's date (`YYYY-MM-DD` as sent).
    pub date: String,
    /// The entry's kind.
    pub txn_type: String,
    /// Its number.
    pub number: Option<String>,
    /// Its description.
    pub description: Option<String>,
    /// Money out.
    pub payment: Option<ReportAmount>,
    /// Money in.
    pub deposit: Option<ReportAmount>,
}

/// The reconciliation detail report (`GET /reports/reconciliation-detail`):
/// the summary and the detail PDFs are both drawn from it.
#[derive(Clone, Debug)]
pub struct ReconReport {
    /// The account's name.
    pub account_name: String,
    /// Its number.
    pub account_number: Option<String>,
    /// The statement date (`YYYY-MM-DD`).
    pub statement_date: String,
    /// The beginning balance.
    pub beginning_balance: Option<ReportAmount>,
    /// The statement's ending balance.
    pub statement_ending_balance: Option<ReportAmount>,
    /// The cleared balance.
    pub cleared_balance: Option<ReportAmount>,
    /// The difference.
    pub difference: Option<ReportAmount>,
    /// Who finished it.
    pub completed_by: Option<String>,
    /// Cleared lines.
    pub cleared: Vec<ReportLine>,
    /// Uncleared lines.
    pub uncleared: Vec<ReportLine>,
    /// Cleared money out.
    pub cleared_payments: Option<ReportAmount>,
    /// Cleared money in.
    pub cleared_deposits: Option<ReportAmount>,
    /// Uncleared money out.
    pub uncleared_payments: Option<ReportAmount>,
    /// Uncleared money in.
    pub uncleared_deposits: Option<ReportAmount>,
}

// ---------------------------------------------------------------- bodies

fn charge_json(c: &StatementCharge) -> Result<Value, ApiError> {
    if !c.amount.is_positive() || !two_places(c.amount) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(json!({
        "amount": wire_2dp(c.amount),
        "date": c.date.to_string(),
        "accountId": c.account.to_string(),
    }))
}

fn two_places(m: Money) -> bool {
    m.as_decimal().scale() <= 2
}

/// A balance as the owner typed it (at most 2 decimals, may be negative).
fn wire_2dp(m: Money) -> String {
    let d = m.as_decimal();
    format!("{d:.2}")
}

fn balance_json(m: Money) -> Result<Value, ApiError> {
    if two_places(m) {
        Ok(Value::String(wire_2dp(m)))
    } else {
        Err(ApiError::Local(LocalValidation::InvalidValue))
    }
}

/// The body of `POST /reconciliations` (pre-check and later lines on, the
/// desktop's choice).
///
/// # Errors
/// `Local(InvalidValue)` for a balance with more than 2 decimals or a
/// charge that is not above zero with at most 2 decimals.
pub fn start_body(s: &ReconStart) -> Result<Value, ApiError> {
    let mut m = Map::new();
    m.insert("accountId".into(), json!(s.account.to_string()));
    m.insert("statementDate".into(), json!(s.statement_date.to_string()));
    m.insert(
        "statementEndingBalance".into(),
        balance_json(s.statement_ending_balance)?,
    );
    if let Some(c) = &s.service_charge {
        m.insert("serviceCharge".into(), charge_json(c)?);
    }
    if let Some(c) = &s.interest {
        m.insert("interest".into(), charge_json(c)?);
    }
    m.insert("includeLaterLines".into(), json!(true));
    m.insert("precheck".into(), json!("bank_feed"));
    Ok(Value::Object(m))
}

/// The body of `PATCH /reconciliations/:id`.
///
/// # Errors
/// `Local(InvalidValue)` for an empty change or a bad amount.
pub fn patch_body(p: &ReconPatch) -> Result<Value, ApiError> {
    let mut m = Map::new();
    if let Some(d) = p.statement_date {
        m.insert("statementDate".into(), json!(d.to_string()));
    }
    if let Some(b) = p.statement_ending_balance {
        m.insert("statementEndingBalance".into(), balance_json(b)?);
    }
    for (key, c) in [
        ("serviceCharge", &p.service_charge),
        ("interest", &p.interest),
    ] {
        match c {
            None => {}
            Some(None) => {
                m.insert(key.into(), Value::Null);
            }
            Some(Some(c)) => {
                m.insert(key.into(), charge_json(c)?);
            }
        }
    }
    if m.is_empty() {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(Value::Object(m))
}

/// The body of `PUT /reconciliations/:id/lines`.
///
/// # Errors
/// `Local(InvalidValue)` for no ticks or more than [`LINES_MAX`].
pub fn lines_body(ticks: &[(JournalLineId, bool)]) -> Result<Value, ApiError> {
    if ticks.is_empty() || ticks.len() > LINES_MAX {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let lines: Vec<Value> = ticks
        .iter()
        .map(|(id, on)| json!({"journalLineId": id.to_string(), "isCleared": on}))
        .collect();
    Ok(json!({ "lines": lines }))
}

// ---------------------------------------------------------------- wire

fn decode(what: &'static str) -> ApiError {
    ApiError::Decode { what }
}

/// An exact amount: a JSON string the strict grammar accepts. A number is
/// refused (it may have been rounded).
fn money_at(v: &Value, key: &str, what: &'static str) -> Result<Money, ApiError> {
    match v.get(key) {
        Some(Value::String(s)) => Money::parse_wire(s).map_err(|_| decode(what)),
        _ => Err(decode(what)),
    }
}

/// `exact` when the engine sends it (U-188 keeps the legacy number under
/// `legacy` for older clients), else `legacy` as an exact string.
fn exact_or(
    v: &Value,
    exact: &str,
    legacy: &str,
    what: &'static str,
) -> Result<Money, ApiError> {
    if v.get(exact).is_some_and(|x| !x.is_null()) {
        money_at(v, exact, what)
    } else {
        money_at(v, legacy, what)
    }
}

fn money_opt(v: &Value, key: &str, what: &'static str) -> Result<Option<Money>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Money::parse_wire(s).map(Some).map_err(|_| decode(what)),
        Some(_) => Err(decode(what)),
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

fn date_at(v: &Value, key: &str) -> Option<CalendarDate> {
    let s = str_at(v, key)?;
    CalendarDate::parse_wire(s.get(..10).unwrap_or(s)).ok()
}

fn stamp_at(v: &Value, key: &str) -> Option<Timestamp> {
    str_at(v, key).and_then(|s| Timestamp::parse_wire(s).ok())
}

fn bool_at(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn number_text(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn side(v: Option<&Value>, what: &'static str) -> Result<ClearedSide, ApiError> {
    let Some(v) = v else {
        return Err(decode(what));
    };
    let count = v
        .get("count")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| decode(what))?;
    Ok(ClearedSide {
        count,
        total: money_at(v, "total", what)?,
    })
}

fn held(v: &Value, prefix: &str, what: &'static str) -> Result<Option<HeldCharge>, ApiError> {
    let Some(amount) = money_opt(v, &format!("{prefix}Amount"), what)? else {
        return Ok(None);
    };
    let (Some(date), Some(account)) = (
        date_at(v, &format!("{prefix}Date")),
        str_at(v, &format!("{prefix}AccountId")).and_then(|s| AccountId::parse(s).ok()),
    ) else {
        return Err(decode(what));
    };
    let posted = str_at(v, &format!("{prefix}TxnId")).and_then(|s| TransactionId::parse(s).ok());
    Ok(Some(HeldCharge {
        charge: StatementCharge {
            amount,
            date,
            account,
        },
        posted,
    }))
}

fn drift_of(v: Option<&Value>) -> Result<Option<BeginningDrift>, ApiError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(d) => Ok(Some(BeginningDrift {
            expected: money_at(d, "expected", "beginning balance drift")?,
            actual: money_at(d, "actual", "beginning balance drift")?,
            difference: money_at(d, "difference", "beginning balance drift")?,
        })),
    }
}

/// The continuity warning. Its figures are exact strings only when the
/// engine sends them so; a warning in JSON numbers (it may have been
/// rounded) is left out rather than shown inexactly.
fn continuity_of(v: Option<&Value>) -> Option<Continuity> {
    let d = v.filter(|d| !d.is_null())?;
    Some(Continuity {
        expected: money_at(d, "expected", "continuity warning").ok()?,
        actual: money_at(d, "actual", "continuity warning").ok()?,
        delta: money_at(d, "delta", "continuity warning").ok()?,
    })
}

fn line_of(v: &Value) -> Result<ReconLine, ApiError> {
    let what = "reconciliation line";
    let journal_line = str_at(v, "journal_line_id")
        .or_else(|| str_at(v, "journalLineId"))
        .and_then(|s| JournalLineId::parse(s).ok())
        .ok_or_else(|| decode(what))?;
    let transaction = str_at(v, "transaction_id")
        .or_else(|| str_at(v, "transactionId"))
        .and_then(|s| TransactionId::parse(s).ok())
        .ok_or_else(|| decode(what))?;
    let date = date_at(v, "txn_date")
        .or_else(|| date_at(v, "txnDate"))
        .ok_or_else(|| decode(what))?;
    let payee = str_at(v, "contact_name")
        .or_else(|| str_at(v, "payee_name_on_check"))
        .or_else(|| str_at(v, "description"))
        .map(str::to_string);
    Ok(ReconLine {
        journal_line,
        transaction,
        cleared: bool_at(v, "is_cleared") || bool_at(v, "isCleared"),
        date,
        txn_type: str_at(v, "txn_type").unwrap_or_default().to_string(),
        number: number_text(v, "check_number").or_else(|| number_text(v, "txn_number")),
        payee,
        amount: money_at(v, "amount", what)?,
        after_statement_date: bool_at(v, "afterStatementDate"),
        from_bank_feed: bool_at(v, "fromBankFeed"),
        likely_duplicate: bool_at(v, "likely_duplicate"),
    })
}

/// Reads a reconciliation object (the value under `reconciliation`).
///
/// # Errors
/// `Decode` for a missing field, an id that isn't a UUID or an amount
/// that isn't an exact string.
pub fn reconciliation_from_value(r: &Value) -> Result<Reconciliation, ApiError> {
    let what = "reconciliation";
    let id = str_at(r, "id")
        .ok_or_else(|| decode(what))
        .and_then(ReconciliationId::parse)?;
    let account = str_at(r, "accountId")
        .and_then(|s| AccountId::parse(s).ok())
        .ok_or_else(|| decode(what))?;
    let lines = match r.get("lines") {
        Some(Value::Array(a)) => Some(a.iter().map(line_of).collect::<Result<Vec<_>, _>>()?),
        _ => None,
    };
    Ok(Reconciliation {
        id,
        account,
        status: ReconStatus::parse(str_at(r, "status")),
        statement_date: date_at(r, "statementDate").ok_or_else(|| decode(what))?,
        beginning_balance: money_at(r, "beginningBalance", what)?,
        statement_ending_balance: money_at(r, "statementEndingBalance", what)?,
        cleared_balance: exact_or(r, "clearedBalanceExact", "clearedBalance", what)?,
        difference: exact_or(r, "differenceExact", "difference", what)?,
        cleared_deposits: side(r.get("clearedDeposits"), what)?,
        cleared_payments: side(r.get("clearedPayments"), what)?,
        service_charge: held(r, "serviceCharge", what)?,
        interest: held(r, "interest", what)?,
        include_later_lines: bool_at(r, "includeLaterLines"),
        drift: drift_of(r.get("beginningBalanceDrift"))?,
        continuity: continuity_of(r.get("continuityWarning")),
        posting_locked: bool_at(r, "postingLocked"),
        started: stamp_at(r, "createdAt"),
        lines,
    })
}

/// Reads `{reconciliation: {...}}`.
///
/// # Errors
/// See [`reconciliation_from_value`].
pub fn reconciliation_from(body: &[u8]) -> Result<Reconciliation, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| decode("reconciliation"))?;
    reconciliation_from_value(
        v.get("reconciliation")
            .ok_or_else(|| decode("reconciliation"))?,
    )
}

/// Reads `{reconciliations: [...]}` (an account's history).
///
/// # Errors
/// `Decode` for a row without an id, status date or a bad id.
pub fn history_from(body: &[u8]) -> Result<Vec<ReconHistoryRow>, ApiError> {
    let what = "reconciliation history";
    let v: Value = serde_json::from_slice(body).map_err(|_| decode(what))?;
    let rows = v
        .get("reconciliations")
        .and_then(Value::as_array)
        .ok_or_else(|| decode(what))?;
    rows.iter()
        .map(|r| {
            Ok(ReconHistoryRow {
                id: str_at(r, "id")
                    .ok_or_else(|| decode(what))
                    .and_then(ReconciliationId::parse)?,
                status: ReconStatus::parse(str_at(r, "status")),
                statement_date: date_at(r, "statementDate").ok_or_else(|| decode(what))?,
                statement_ending_balance: money_opt(r, "statementEndingBalance", what)
                    .ok()
                    .flatten(),
                completed: stamp_at(r, "completedAt"),
                started: stamp_at(r, "createdAt"),
            })
        })
        .collect()
}

/// Reads the discrepancy report.
///
/// # Errors
/// `Decode` when it isn't the documented shape or an amount is a number.
pub fn discrepancy_from(body: &[u8]) -> Result<DiscrepancyReport, ApiError> {
    let what = "reconciliation discrepancy";
    let v: Value = serde_json::from_slice(body).map_err(|_| decode(what))?;
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| decode(what))?;
    let rows = rows
        .iter()
        .map(|r| {
            Ok(DiscrepancyRow {
                kind: str_at(r, "kind").ok_or_else(|| decode(what))?.to_string(),
                statement_date: date_at(r, "statementDate"),
                txn_date: date_at(r, "txnDate"),
                txn_type: str_at(r, "txnType").map(str::to_string),
                number: number_text(r, "number"),
                payee: str_at(r, "payee").map(str::to_string),
                reconciled_amount: money_opt(r, "reconciledAmount", what)?,
                effect: money_opt(r, "effect", what)?,
                changed_at: stamp_at(r, "changedAt"),
                changed_by: str_at(r, "changedBy").map(str::to_string),
                fields: r
                    .get("fields")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                transaction: str_at(r, "transactionId").and_then(|s| TransactionId::parse(s).ok()),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    Ok(DiscrepancyReport {
        rows,
        drift: drift_of(v.get("drift"))?,
    })
}

fn amount_of(v: &Value, key: &str) -> Option<ReportAmount> {
    let raw = v.get(key)?;
    if raw.is_null() {
        return None;
    }
    serde_json::from_value(raw.clone()).ok()
}

fn report_line(v: &Value) -> ReportLine {
    ReportLine {
        date: str_at(v, "txnDate")
            .map(|s| s.get(..10).unwrap_or(s).to_string())
            .unwrap_or_default(),
        txn_type: str_at(v, "txnType").unwrap_or_default().to_string(),
        number: number_text(v, "txnNumber"),
        description: str_at(v, "description").map(str::to_string),
        payment: amount_of(v, "payment"),
        deposit: amount_of(v, "deposit"),
    }
}

/// Reads the reconciliation detail report.
///
/// # Errors
/// `Decode` when the header is missing.
pub fn report_from(body: &[u8]) -> Result<ReconReport, ApiError> {
    let what = "reconciliation report";
    let v: Value = serde_json::from_slice(body).map_err(|_| decode(what))?;
    let r = v.get("reconciliation").ok_or_else(|| decode(what))?;
    let lines = |k: &str| {
        v.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().map(report_line).collect())
            .unwrap_or_default()
    };
    let totals = v.get("totals").cloned().unwrap_or(Value::Null);
    Ok(ReconReport {
        account_name: str_at(r, "accountName").unwrap_or_default().to_string(),
        account_number: str_at(r, "accountNumber").map(str::to_string),
        statement_date: str_at(r, "statementDate")
            .map(|s| s.get(..10).unwrap_or(s).to_string())
            .ok_or_else(|| decode(what))?,
        beginning_balance: amount_of(r, "beginningBalance"),
        statement_ending_balance: amount_of(r, "statementEndingBalance"),
        cleared_balance: amount_of(r, "clearedBalance"),
        difference: amount_of(r, "difference"),
        completed_by: str_at(r, "completedBy").map(str::to_string),
        cleared: lines("cleared"),
        uncleared: lines("uncleared"),
        cleared_payments: amount_of(&totals, "clearedPayments"),
        cleared_deposits: amount_of(&totals, "clearedDeposits"),
        uncleared_payments: amount_of(&totals, "unclearedPayments"),
        uncleared_deposits: amount_of(&totals, "unclearedDeposits"),
    })
}

// ---------------------------------------------------------------- routes

/// The reconciliation routes of the open company.
#[derive(Clone, Copy, Debug)]
pub struct Reconciliations<'a>(&'a Session);

impl Session {
    /// Reconcile (P27).
    #[must_use]
    pub fn reconciliations(&self) -> Reconciliations<'_> {
        Reconciliations(self)
    }
}

fn id_call(op: &Operation, id: ReconciliationId, tail: &str) -> Call {
    let path = if tail.is_empty() {
        format!("/api/v1/banking/reconciliations/{id}")
    } else {
        format!("/api/v1/banking/reconciliations/{id}/{tail}")
    };
    Call::new(op.method(), op.route(), path)
}

/// True when the engine has everything the Reconcile window needs
/// (`reconcile-exact-v1`, `reconcile-begin-v1`, `reconcile-undo-v2`).
#[must_use]
pub fn screen_supported(caps: &Capabilities) -> bool {
    caps.has(Capabilities::RECONCILE_EXACT)
        && caps.has(Capabilities::RECONCILE_BEGIN)
        && caps.has(Capabilities::RECONCILE_UNDO)
}

impl Reconciliations<'_> {
    fn need(self) -> Result<(), ApiError> {
        if screen_supported(&self.0.client().capabilities()) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    fn with_password(
        mut body: Value,
        auth: Option<&ClosedPeriodAuthorization>,
    ) -> Result<Vec<u8>, ApiError> {
        if let (Some(a), Value::Object(m)) = (auth, &mut body) {
            m.insert("closingPassword".into(), json!(a.expose()));
        }
        json_body(&body)
    }

    /// An account's reconciliations, newest statement first.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn history(&self, account: AccountId) -> Result<Vec<ReconHistoryRow>, ApiError> {
        let mut call = Call::at(Method::Get, RECONCILIATIONS.route());
        call.query = vec![("account_id".into(), account.to_string())];
        let reply = self.0.send(&RECONCILIATIONS, call, None).await?;
        history_from(&reply.body)
    }

    /// Begin: starts the worksheet (bank-file items pre-checked, later
    /// lines included).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the reconcile capabilities; the body's
    /// local checks; `Conflict` `RECONCILIATION_IN_PROGRESS`; any
    /// [`ApiError`].
    pub async fn start(&self, s: &ReconStart) -> Result<Reconciliation, ApiError> {
        self.need()?;
        let body = json_body(&start_body(s)?)?;
        let call = Call::at(Method::Post, RECON_START.route()).json_body(body);
        let reply = self.0.send(&RECON_START, call, None).await?;
        reconciliation_from(&reply.body)
    }

    /// The worksheet with its lines.
    ///
    /// # Errors
    /// `Local(Unsupported)`; any [`ApiError`].
    pub async fn get(&self, id: ReconciliationId) -> Result<Reconciliation, ApiError> {
        self.need()?;
        let call = id_call(&RECON_GET, id, "");
        let reply = self.0.send(&RECON_GET, call, None).await?;
        reconciliation_from(&reply.body)
    }

    /// Ticks and unticks lines in one batch; the answer has the header and
    /// totals only.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no ticks or too many; `Conflict`
    /// `RECONCILIATION_NOT_IN_PROGRESS`; any [`ApiError`].
    pub async fn tick(
        &self,
        id: ReconciliationId,
        ticks: &[(JournalLineId, bool)],
    ) -> Result<Reconciliation, ApiError> {
        self.need()?;
        let body = json_body(&lines_body(ticks)?)?;
        let mut call = id_call(&RECON_LINES, id, "lines").json_body(body);
        call.query = vec![("view".into(), "summary".into())];
        let reply = self.0.send(&RECON_LINES, call, None).await?;
        reconciliation_from(&reply.body)
    }

    /// Changes the statement info.
    ///
    /// # Errors
    /// The body's local checks; `Conflict` `RECONCILE_CHARGE_POSTED` or
    /// `RECONCILIATION_NOT_IN_PROGRESS`; any [`ApiError`].
    pub async fn patch(
        &self,
        id: ReconciliationId,
        p: &ReconPatch,
    ) -> Result<Reconciliation, ApiError> {
        self.need()?;
        let body = json_body(&patch_body(p)?)?;
        let call = id_call(&RECON_PATCH, id, "").json_body(body);
        let reply = self.0.send(&RECON_PATCH, call, None).await?;
        reconciliation_from(&reply.body)
    }

    /// Pulls in lines entered since Begin (pre-checking new bank-file
    /// items) and drops lines that no longer exist.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn refresh(&self, id: ReconciliationId) -> Result<(), ApiError> {
        self.need()?;
        let body = json_body(&json!({"includeLaterLines": true, "precheck": "bank_feed"}))?;
        let call = id_call(&RECON_REFRESH, id, "refresh").json_body(body);
        self.0.send(&RECON_REFRESH, call, None).await?;
        Ok(())
    }

    /// Finish: posts the pending charges and marks the ticked lines
    /// reconciled.
    ///
    /// # Errors
    /// `Conflict` `RECONCILE_DIFFERENCE_NOT_ZERO`, `ClosedPeriod` and the
    /// closing-password errors; any [`ApiError`].
    pub async fn complete(
        &self,
        id: ReconciliationId,
        auth: Option<&ClosedPeriodAuthorization>,
    ) -> Result<(), ApiError> {
        self.need()?;
        let body = Self::with_password(json!({}), auth)?;
        let call = id_call(&RECON_COMPLETE, id, "complete").json_body(body);
        self.0.send(&RECON_COMPLETE, call, None).await?;
        Ok(())
    }

    /// Undo the most recent finished reconciliation (Admin only).
    ///
    /// # Errors
    /// `Forbidden` `RECONCILE_ADMIN_REQUIRED`; `Conflict`
    /// `RECONCILIATION_NOT_LATEST` / `RECONCILIATION_IN_PROGRESS`;
    /// `ClosedPeriod`; any [`ApiError`].
    pub async fn undo(
        &self,
        id: ReconciliationId,
        mode: UndoMode,
        auth: Option<&ClosedPeriodAuthorization>,
    ) -> Result<(), ApiError> {
        self.need()?;
        let body = Self::with_password(json!({"mode": mode.as_str()}), auth)?;
        let call = id_call(&RECON_UNDO, id, "undo").json_body(body);
        self.0.send(&RECON_UNDO, call, None).await?;
        Ok(())
    }

    /// Discard a reconciliation in progress ("Start over").
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn cancel(&self, id: ReconciliationId) -> Result<(), ApiError> {
        self.need()?;
        let call = id_call(&RECON_CANCEL, id, "cancel");
        self.0.send(&RECON_CANCEL, call, None).await?;
        Ok(())
    }

    /// The detail report of a reconciliation (also the summary's data).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn report(&self, id: ReconciliationId) -> Result<ReconReport, ApiError> {
        let mut call = Call::at(Method::Get, RECON_DETAIL_REPORT.route());
        call.query = vec![("reconciliation_id".into(), id.to_string())];
        let reply = self.0.send(&RECON_DETAIL_REPORT, call, None).await?;
        report_from(&reply.body)
    }

    /// The discrepancy report of an account.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `reconcile-discrepancy-v1`; any
    /// [`ApiError`].
    pub async fn discrepancy(&self, account: AccountId) -> Result<DiscrepancyReport, ApiError> {
        if !self
            .0
            .client()
            .capabilities()
            .has(Capabilities::RECONCILE_DISCREPANCY)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let mut call = Call::at(Method::Get, RECON_DISCREPANCY.route());
        call.query = vec![("account_id".into(), account.to_string())];
        let reply = self.0.send(&RECON_DISCREPANCY, call, None).await?;
        discrepancy_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "reconciliations_tests.rs"]
mod tests;
