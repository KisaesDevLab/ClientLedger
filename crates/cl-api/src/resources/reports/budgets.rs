//! Budgets (U-203, `budgets-exact-v1`, Q-65, Q-P28n): simple P&L budgets of
//! twelve fiscal periods, company-scoped and versioned by the engine.
//!
//! Every total is the engine's: a line's `totalExact`, the per-period
//! section totals and net income (`totalsExact`) come back with every read
//! and every write. Cells go out as decimal strings of owner-typed
//! [`Money`] (at most two places, checked here before sending); fill from
//! actuals and adjust by percent run in the engine, which rounds each cell
//! to the cent, half away from zero. A write at a version that is no longer
//! current is 409 `STALE_VERSION` ([`BudgetRefusal::Stale`]): the screen
//! offers Reload.

use std::str::FromStr;

use cl_core::CalendarDate;
use cl_core::Money;
use cl_core::ids::{AccountId, TagGroupId};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

use crate::IdempotencyKey;
use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::resources::operations::{
    BUDGET_ADJUST, BUDGET_COPY, BUDGET_DELETE, BUDGET_FILL, BUDGET_LINES, BUDGET_LINES_PUT,
    BUDGET_PUT, BUDGET_VS_ACTUAL, BUDGETS_CREATE, BUDGETS_LIST,
};
use crate::session::Session;
use crate::wire::Capabilities;

/// Budget periods (month1..month12 = fiscal periods 1..12).
pub const PERIODS: usize = 12;

/// One budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Budget {
    /// Its id.
    pub id: String,
    /// Name (owner data).
    pub name: String,
    /// The fiscal year (FY N starts in calendar year N at the company's
    /// fiscal start month).
    pub fiscal_year: i32,
    /// The fiscal year's first day.
    pub fiscal_year_start: Option<CalendarDate>,
    /// A budget for one tag (its actuals are that tag's lines).
    pub tag_id: Option<String>,
    /// Version for the next change.
    pub version: u64,
}

/// One budget line: an account's twelve cells and the engine's total.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetLine {
    /// The account.
    pub account_id: AccountId,
    /// Its name (owner data).
    pub account_name: String,
    /// Its number.
    pub account_number: Option<String>,
    /// Its type (`revenue`, `cogs`, `expense`, `other_revenue`,
    /// `other_expense`).
    pub account_type: String,
    /// The cells, exact.
    pub periods: [Decimal; PERIODS],
    /// The engine's total of the line.
    pub total: Decimal,
}

/// A section's (or net income's) per-period totals and total.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeriodTotals {
    /// One per period.
    pub periods: Vec<Decimal>,
    /// The year.
    pub total: Decimal,
}

/// The engine's totals of a budget's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetTotals {
    /// Income.
    pub revenue: PeriodTotals,
    /// Cost of goods sold.
    pub cogs: PeriodTotals,
    /// Expenses.
    pub expenses: PeriodTotals,
    /// Other income.
    pub other_revenue: PeriodTotals,
    /// Other expenses.
    pub other_expenses: PeriodTotals,
    /// Net income (income + other income - cost of goods sold - expenses -
    /// other expenses), computed by the engine.
    pub net_income: PeriodTotals,
}

/// A budget's lines as the engine answers a read or a write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetLines {
    /// The budget, at its new version.
    pub budget: Budget,
    /// The lines.
    pub lines: Vec<BudgetLine>,
    /// The engine's totals.
    pub totals: BudgetTotals,
}

/// One line to save: every one of the account's twelve cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetLineWrite {
    /// The account (a P&L account of the company).
    pub account_id: AccountId,
    /// The cells, as the owner typed them (at most two places).
    pub periods: [Money; PERIODS],
}

/// A new budget.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewBudget {
    /// Name.
    pub name: String,
    /// The fiscal year.
    pub fiscal_year: i32,
}

/// Why the engine refused a budget change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetRefusal {
    /// Someone changed the budget since it was read (409 `STALE_VERSION`).
    Stale,
    /// The company already has a budget for that fiscal year (409).
    YearTaken,
    /// A line the engine refused (400 `BUDGET_LINE_INVALID`): not a P&L
    /// account of the company, more than two places, a period outside 1-12.
    LineInvalid,
}

/// The budget refusal in `e`, if it is one.
#[must_use]
pub fn budget_refusal(e: &ApiError) -> Option<BudgetRefusal> {
    match e {
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => Some(BudgetRefusal::Stale),
        ApiError::Conflict { code: Some(c), .. } if c.as_str() == "STALE_VERSION" => {
            Some(BudgetRefusal::Stale)
        }
        // The engine's "a budget for this year already exists" has no code
        // of its own; an idempotency or busy 409 is not that (S10).
        ApiError::Conflict {
            reason: ConflictReason::AlreadyExists,
            ..
        } => Some(BudgetRefusal::YearTaken),
        ApiError::Conflict { code: Some(c), .. } if c.as_str() == "BUDGET_YEAR_TAKEN" => {
            Some(BudgetRefusal::YearTaken)
        }
        ApiError::Conflict {
            code: None, reason, ..
        } if *reason != ConflictReason::Busy => Some(BudgetRefusal::YearTaken),
        ApiError::Validation(v)
            if v.code()
                .is_some_and(|c| c.as_str() == "BUDGET_LINE_INVALID") =>
        {
            Some(BudgetRefusal::LineInvalid)
        }
        _ => None,
    }
}

const WHAT: &str = "budget";

fn shape() -> ApiError {
    ApiError::Decode { what: WHAT }
}

fn exact(v: &Value) -> Result<Decimal, ApiError> {
    v.as_str()
        .and_then(|s| Decimal::from_str_exact(s).ok())
        .ok_or_else(shape)
}

fn text(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// A budget object (`{budget}` or bare).
fn budget(v: &Value) -> Result<Budget, ApiError> {
    let v = v.get("budget").unwrap_or(v);
    let fiscal_year = v
        .get("fiscalYear")
        .and_then(Value::as_i64)
        .and_then(|y| i32::try_from(y).ok())
        .ok_or_else(shape)?;
    // A date, or a timestamp whose first ten characters are the date.
    let fiscal_year_start = text(v, "fiscalYearStart")
        .and_then(|s| s.get(..10).and_then(|d| CalendarDate::parse_wire(d).ok()));
    Ok(Budget {
        id: text(v, "id").ok_or_else(shape)?,
        name: text(v, "name").unwrap_or_default(),
        fiscal_year,
        fiscal_year_start,
        tag_id: text(v, "tagId"),
        version: v.get("version").and_then(Value::as_u64).ok_or_else(shape)?,
    })
}

fn period_totals(v: &Value) -> Result<PeriodTotals, ApiError> {
    let periods = v
        .get("periodsExact")
        .and_then(Value::as_array)
        .ok_or_else(shape)?
        .iter()
        .map(exact)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PeriodTotals {
        periods,
        total: exact(v.get("totalExact").unwrap_or(&Value::Null))?,
    })
}

fn line(v: &Value) -> Result<BudgetLine, ApiError> {
    let account_id = v
        .get("account_id")
        .or_else(|| v.get("accountId"))
        .and_then(Value::as_str)
        .and_then(|s| AccountId::parse(s).ok())
        .ok_or_else(shape)?;
    let t = period_totals(v)?;
    let periods: [Decimal; PERIODS] = t.periods.try_into().map_err(|_| shape())?;
    Ok(BudgetLine {
        account_id,
        account_name: text(v, "account_name").unwrap_or_default(),
        account_number: text(v, "account_number"),
        account_type: text(v, "account_type").unwrap_or_default(),
        periods,
        total: t.total,
    })
}

/// The lines view (`GET /:id/lines` and every write's answer).
///
/// # Errors
/// [`ApiError::Decode`] when an exact field is missing (never a fallback to
/// the number columns).
pub fn lines_view(v: &Value) -> Result<BudgetLines, ApiError> {
    let mut b = budget(v.get("budget").ok_or_else(shape)?)?;
    if let Some(ver) = v.get("version").and_then(Value::as_u64) {
        b.version = ver;
    }
    let lines = v
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(shape)?
        .iter()
        .map(line)
        .collect::<Result<Vec<_>, _>>()?;
    let t = v.get("totalsExact").ok_or_else(shape)?;
    let pick = |k: &str| period_totals(t.get(k).ok_or_else(shape)?);
    Ok(BudgetLines {
        budget: b,
        lines,
        totals: BudgetTotals {
            revenue: pick("revenue")?,
            cogs: pick("cogs")?,
            expenses: pick("expenses")?,
            other_revenue: pick("otherRevenue")?,
            other_expenses: pick("otherExpenses")?,
            net_income: pick("netIncome")?,
        },
    })
}

/// The body of `PUT /:id/lines`. A cell with more than two places is
/// refused here (nothing sent).
///
/// # Errors
/// `Local(InvalidValue)` for a cell with more than two places.
pub fn lines_body(version: u64, lines: &[BudgetLineWrite]) -> Result<Value, ApiError> {
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        let mut m = serde_json::Map::new();
        m.insert("accountId".into(), Value::from(l.account_id.to_string()));
        for (i, c) in l.periods.iter().enumerate() {
            let d = c.as_decimal();
            if d.normalize().scale() > 2 {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            m.insert(format!("month{}", i + 1), Value::from(c.to_wire()));
        }
        out.push(Value::Object(m));
    }
    Ok(serde_json::json!({"version": version, "lines": out}))
}

/// A percent for adjust (`"5"`, `"-2.5"`): -100 or more, at most two places.
///
/// # Errors
/// `Local(InvalidValue)` otherwise.
pub fn percent_text(p: Decimal) -> Result<String, ApiError> {
    if p < Decimal::from(-100) || p.normalize().scale() > 2 {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(p.normalize().to_string())
}

/// Budget vs. Actual options (`exact=1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BvaQuery {
    /// Each budget period's dates, in order (at most twelve; 4-4-5 from
    /// the desktop's fiscal calendar).
    pub ranges: Vec<(CalendarDate, CalendarDate)>,
    /// The budget period of the first range (1..12).
    pub first_period: u8,
    /// `accrual` or `cash`.
    pub basis: Option<&'static str>,
    /// `include`, `exclude` or `only`.
    pub adjusting: Option<&'static str>,
    /// Split each account by this group's tags.
    pub tag_split: Option<TagGroupId>,
}

/// The query of a Budget vs. Actual read.
#[must_use]
pub fn bva_query(q: &BvaQuery) -> Vec<(String, String)> {
    let mut out = vec![("exact".to_string(), "1".to_string())];
    if !q.ranges.is_empty() {
        let r = q
            .ranges
            .iter()
            .map(|(a, b)| format!("{}..{}", a.to_wire(), b.to_wire()))
            .collect::<Vec<_>>()
            .join(",");
        out.push(("period_ranges".into(), r));
    }
    out.push(("first_period".into(), q.first_period.max(1).to_string()));
    if let Some(b) = q.basis {
        out.push(("basis".into(), b.to_string()));
    }
    if let Some(a) = q.adjusting {
        out.push(("adjusting".into(), a.to_string()));
    }
    if let Some(g) = q.tag_split {
        out.push(("tag_split".into(), "1".into()));
        out.push(("tag_group_id".into(), g.to_string()));
    }
    out
}

/// Budgets of a session (`session.budgets()`).
#[derive(Clone, Copy, Debug)]
pub struct Budgets<'a>(&'a Session);

impl Session {
    /// The open company's budgets (P28 WP4).
    #[must_use]
    pub fn budgets(&self) -> Budgets<'_> {
        Budgets(self)
    }
}

fn path(route: &str, id: &str) -> String {
    route.replace("{id}", id)
}

impl Budgets<'_> {
    fn require(self) -> Result<(), ApiError> {
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::BUDGETS_EXACT)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    async fn json(
        &self,
        op: &'static crate::resources::operations::Operation,
        call: Call,
    ) -> Result<Value, ApiError> {
        let reply = self.0.send(op, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| shape())
    }

    /// The company's budgets.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `budgets-exact-v1`; any [`ApiError`].
    pub async fn list(&self) -> Result<Vec<Budget>, ApiError> {
        self.require()?;
        let v = self
            .json(&BUDGETS_LIST, Call::at(Method::Get, BUDGETS_LIST.route()))
            .await?;
        v.get("budgets")
            .and_then(Value::as_array)
            .ok_or_else(shape)?
            .iter()
            .map(budget)
            .collect()
    }

    /// Creates a budget for a fiscal year.
    ///
    /// # Errors
    /// [`BudgetRefusal::YearTaken`] when the year has one; any [`ApiError`].
    pub async fn create(&self, b: &NewBudget, key: IdempotencyKey) -> Result<Budget, ApiError> {
        self.require()?;
        let body =
            serde_json::to_vec(b).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let mut call = Call::at(Method::Post, BUDGETS_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        budget(&self.json(&BUDGETS_CREATE, call).await?)
    }

    /// Renames a budget read at `version`.
    ///
    /// # Errors
    /// [`BudgetRefusal::Stale`]; any [`ApiError`].
    pub async fn rename(
        &self,
        id: &str,
        version: u64,
        name: &str,
        key: IdempotencyKey,
    ) -> Result<Budget, ApiError> {
        self.require()?;
        let body = serde_json::to_vec(&serde_json::json!({"name": name, "version": version}))
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let mut call = Call::new(
            Method::Put,
            BUDGET_PUT.route(),
            path(BUDGET_PUT.route(), id),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        budget(&self.json(&BUDGET_PUT, call).await?)
    }

    /// Deletes a budget and its lines.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.require()?;
        let call = Call::new(
            Method::Delete,
            BUDGET_DELETE.route(),
            path(BUDGET_DELETE.route(), id),
        );
        self.0.send(&BUDGET_DELETE, call, None).await.map(drop)
    }

    /// The budget's lines and the engine's totals.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn lines(&self, id: &str) -> Result<BudgetLines, ApiError> {
        self.require()?;
        let call = Call::new(
            Method::Get,
            BUDGET_LINES.route(),
            path(BUDGET_LINES.route(), id),
        );
        lines_view(&self.json(&BUDGET_LINES, call).await?)
    }

    /// Saves lines (each replaces its account's twelve cells; other
    /// accounts keep theirs) at `version`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a cell with more than two places (nothing
    /// sent); [`budget_refusal`] for the engine's; any [`ApiError`].
    pub async fn save_lines(
        &self,
        id: &str,
        version: u64,
        lines: &[BudgetLineWrite],
        key: IdempotencyKey,
    ) -> Result<BudgetLines, ApiError> {
        self.require()?;
        let body = serde_json::to_vec(&lines_body(version, lines)?)
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let mut call = Call::new(
            Method::Put,
            BUDGET_LINES_PUT.route(),
            path(BUDGET_LINES_PUT.route(), id),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        lines_view(&self.json(&BUDGET_LINES_PUT, call).await?)
    }

    /// Fills every P&L account from the twelve months before the budget's
    /// year (the engine's calendar months, rounded to the cent).
    ///
    /// # Errors
    /// As [`Budgets::save_lines`].
    pub async fn fill_from_actuals(
        &self,
        id: &str,
        version: u64,
        basis: Option<&'static str>,
        key: IdempotencyKey,
    ) -> Result<BudgetLines, ApiError> {
        self.require()?;
        let mut body = serde_json::json!({"version": version});
        if let Some(b) = basis {
            body["basis"] = Value::from(b);
        }
        let mut call = Call::new(
            Method::Post,
            BUDGET_FILL.route(),
            path(BUDGET_FILL.route(), id),
        )
        .json_body(serde_json::to_vec(&body).map_err(|_| shape())?);
        call.idempotency_key = Some(key);
        lines_view(&self.json(&BUDGET_FILL, call).await?)
    }

    /// Replaces the lines with another budget's.
    ///
    /// # Errors
    /// As [`Budgets::save_lines`].
    pub async fn copy_from(
        &self,
        id: &str,
        version: u64,
        source_id: &str,
        key: IdempotencyKey,
    ) -> Result<BudgetLines, ApiError> {
        self.require()?;
        let p = path(BUDGET_COPY.route(), id).replace("{sourceId}", source_id);
        let mut call = Call::new(Method::Post, BUDGET_COPY.route(), p).json_body(
            serde_json::to_vec(&serde_json::json!({"version": version})).map_err(|_| shape())?,
        );
        call.idempotency_key = Some(key);
        lines_view(&self.json(&BUDGET_COPY, call).await?)
    }

    /// Multiplies every cell by (1 + percent / 100), rounded to the cent by
    /// the engine. One key per click: a retried adjust is replayed, never
    /// applied twice.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a percent under -100 or with more than two
    /// places; as [`Budgets::save_lines`].
    pub async fn adjust_by_percent(
        &self,
        id: &str,
        version: u64,
        percent: Decimal,
        key: IdempotencyKey,
    ) -> Result<BudgetLines, ApiError> {
        self.require()?;
        let body = serde_json::json!({"version": version, "percent": percent_text(percent)?});
        let mut call = Call::new(
            Method::Post,
            BUDGET_ADJUST.route(),
            path(BUDGET_ADJUST.route(), id),
        )
        .json_body(serde_json::to_vec(&body).map_err(|_| shape())?);
        call.idempotency_key = Some(key);
        lines_view(&self.json(&BUDGET_ADJUST, call).await?)
    }

    /// The raw exact Budget vs. Actual answer (the report window turns it
    /// into a table through [`super::adapters::bva::table`]).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn vs_actual(&self, id: &str, q: &BvaQuery) -> Result<Value, ApiError> {
        self.require()?;
        let mut call = Call::new(
            Method::Get,
            BUDGET_VS_ACTUAL.route(),
            path(BUDGET_VS_ACTUAL.route(), id),
        );
        call.query = bva_query(q);
        self.json(&BUDGET_VS_ACTUAL, call).await
    }
}

/// Parses an owner's percent ("5", "-2.5", "5%").
///
/// # Errors
/// `Local(InvalidValue)` for anything else.
pub fn parse_percent(s: &str) -> Result<Decimal, ApiError> {
    let t = s.trim().trim_end_matches('%').trim();
    let d = Decimal::from_str(t).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    percent_text(d)?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ACCT: &str = "11111111-1111-4111-8111-111111111111";

    fn view() -> Value {
        let p = |x: &str| {
            let mut v = vec![json!(x)];
            v.extend(std::iter::repeat_n(json!("0.0000"), 11));
            v
        };
        let t = |x: &str| json!({"periodsExact": p(x), "totalExact": x});
        json!({
            "message": "Lines saved",
            "budget": {"id": "b1", "name": "FY2027", "fiscalYear": 2027,
                       "fiscalYearStart": "2027-01-01", "tagId": null, "version": 2},
            "version": 3,
            "lines": [{"account_id": ACCT, "account_name": "Sales", "account_number": "4000",
                       "account_type": "revenue", "periodsExact": p("1000.1000"),
                       "totalExact": "1000.1000"}],
            "totalsExact": {"revenue": t("1000.1000"), "cogs": t("0.0000"), "expenses": t("0.0000"),
                            "otherRevenue": t("0.0000"), "otherExpenses": t("0.0000"),
                            "netIncome": t("1000.1000")}
        })
    }

    #[test]
    fn the_lines_view_reads_exact_strings_and_the_new_version() {
        let l = lines_view(&view()).expect("view");
        assert_eq!(l.budget.version, 3, "the write's version wins");
        assert_eq!(
            l.budget.fiscal_year_start,
            CalendarDate::from_ymd(2027, 1, 1).ok()
        );
        assert_eq!(l.lines[0].periods[0].to_string(), "1000.1000");
        assert_eq!(l.lines[0].total.to_string(), "1000.1000");
        assert_eq!(l.totals.net_income.total.to_string(), "1000.1000");
    }

    #[test]
    fn a_missing_exact_field_is_refused_never_read_from_the_numbers() {
        let mut v = view();
        v["lines"][0]
            .as_object_mut()
            .expect("o")
            .remove("periodsExact");
        assert!(lines_view(&v).is_err());
        let mut v = view();
        v["totalsExact"]
            .as_object_mut()
            .expect("o")
            .remove("netIncome");
        assert!(lines_view(&v).is_err());
    }

    #[test]
    fn cells_go_out_as_decimal_strings_and_three_places_are_refused() {
        let id = AccountId::parse(ACCT).expect("id");
        let mut periods = [Money::ZERO; PERIODS];
        periods[0] = Money::parse_wire("1234.5").expect("m");
        let body = lines_body(
            4,
            &[BudgetLineWrite {
                account_id: id,
                periods,
            }],
        )
        .expect("body");
        assert_eq!(body["version"], 4);
        assert_eq!(body["lines"][0]["month1"], "1234.5000");
        assert_eq!(body["lines"][0]["month12"], "0.0000");
        assert!(
            body["lines"][0]["month1"].is_string(),
            "never a JSON number"
        );
        periods[1] = Money::parse_wire("0.125").expect("m");
        assert!(
            lines_body(
                4,
                &[BudgetLineWrite {
                    account_id: id,
                    periods
                }]
            )
            .is_err()
        );
    }

    #[test]
    fn percents_are_checked_before_sending() {
        assert_eq!(parse_percent("5").expect("p").to_string(), "5");
        assert_eq!(
            percent_text(parse_percent("-2.5%").expect("p")).expect("t"),
            "-2.5"
        );
        assert!(parse_percent("-101").is_err());
        assert!(parse_percent("1.005").is_err());
        assert!(parse_percent("abc").is_err());
    }

    #[test]
    fn refusals_are_recognised() {
        let stale = crate::engine_refusal(
            409,
            &json!({"error": {"message": "x", "code": "STALE_VERSION", "details": {"current": 4}}}),
        );
        assert_eq!(budget_refusal(&stale), Some(BudgetRefusal::Stale));
        let taken = crate::engine_refusal(
            409,
            &json!({"error": {"message": "A budget for this fiscal year already exists"}}),
        );
        assert_eq!(budget_refusal(&taken), Some(BudgetRefusal::YearTaken));
        let bad = crate::engine_refusal(
            400,
            &json!({"error": {"message": "x", "code": "BUDGET_LINE_INVALID", "details": {"index": 0, "field": "month1"}}}),
        );
        assert_eq!(budget_refusal(&bad), Some(BudgetRefusal::LineInvalid));
        assert_eq!(budget_refusal(&ApiError::Timeout), None);
    }

    #[test]
    fn the_bva_query_sends_explicit_ranges() {
        let d = |m, dd| CalendarDate::from_ymd(2026, m, dd).expect("d");
        let q = BvaQuery {
            ranges: vec![(d(1, 1), d(1, 28)), (d(1, 29), d(2, 25))],
            first_period: 1,
            basis: Some("cash"),
            adjusting: None,
            tag_split: None,
        };
        let v: Vec<String> = bva_query(&q)
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            v,
            [
                "exact=1",
                "period_ranges=2026-01-01..2026-01-28,2026-01-29..2026-02-25",
                "first_period=1",
                "basis=cash"
            ]
        );
    }
}
